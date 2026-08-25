//! Android driver over `adb`.
//!
//! Works from a host with platform-tools installed, or on the device itself
//! from Termux after `adb connect localhost:<wireless-debugging-port>`.
//! Screenshots come from `screencap -p`; input goes through `input …`;
//! the UI tree comes from `uiautomator dump`.

use std::path::PathBuf;
use std::time::Duration;

use crate::config::AndroidConfig;
use crate::consent::AppIdentity;
use crate::driver::{
    AppAction, Button, Driver, DriverError, Point, RawFrame, ScrollDir, TargetInfo, TargetKind,
    UiNode,
};
use crate::drivers::device_elements;
use crate::elements::{
    ActionReceipt, AppInfo, AppState, ElementAction, ElementCaps, ElementDriver, ElementNode,
    StateOpts,
};
use crate::keys::{Key, KeyCombo, NamedKey};
use crate::process::{self, sh_quote};

const SHOT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AndroidDriver {
    adb: PathBuf,
    serial: String,
    size: Option<(u32, u32)>,
    /// Last `computer_app_state` tree, so `#index` keeps meaning something.
    element_nodes: Vec<ElementNode>,
}

fn sdk_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(root) = std::env::var(var)
            && !root.trim().is_empty()
        {
            dirs.push(PathBuf::from(root.trim()).join("platform-tools"));
        }
    }
    if let Some(home) = process::home() {
        dirs.push(home.join("Library/Android/sdk/platform-tools"));
        dirs.push(home.join("Android/Sdk/platform-tools"));
        dirs.push(home.join("AppData/Local/Android/Sdk/platform-tools"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA")
        && !local.trim().is_empty()
    {
        dirs.push(PathBuf::from(local.trim()).join("Android/Sdk/platform-tools"));
    }
    if let Ok(prefix) = std::env::var("PREFIX")
        && !prefix.trim().is_empty()
    {
        // Termux.
        dirs.push(PathBuf::from(prefix.trim()).join("bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/android-sdk/platform-tools"));
    dirs
}

/// Parse `adb devices` output into `(serial, state)` pairs.
pub fn parse_devices(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?;
            let state = parts.next()?;
            Some((serial.to_string(), state.to_string()))
        })
        .collect()
}

impl AndroidDriver {
    pub fn new(cfg: &AndroidConfig) -> Result<Self, DriverError> {
        let adb = process::find_binary(&cfg.adb, "adb", &sdk_candidates()).ok_or_else(|| {
            DriverError::Unavailable(
                "adb was not found; install Android platform-tools (or set [android].adb in ~/.codewhale/computer-use.toml)".to_string(),
            )
        })?;
        let mut driver = Self {
            adb,
            serial: cfg.serial.trim().to_string(),
            size: None,
            element_nodes: Vec::new(),
        };
        if driver.serial.is_empty() {
            let devices = driver.list_devices()?;
            let ready: Vec<&(String, String)> =
                devices.iter().filter(|(_, s)| s == "device").collect();
            match ready.as_slice() {
                [] => {
                    let mut msg = "no Android device is connected (adb devices lists none online); enable USB debugging, or `adb connect host:port` for wireless debugging".to_string();
                    if !devices.is_empty() {
                        msg.push_str(&format!(
                            "; seen: {}",
                            devices
                                .iter()
                                .map(|(s, st)| format!("{s} ({st})"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    return Err(DriverError::Unavailable(msg));
                }
                [(serial, _)] => driver.serial = serial.clone(),
                many => {
                    return Err(DriverError::Unavailable(format!(
                        "several Android devices are connected ({}); set [android].serial in ~/.codewhale/computer-use.toml or ANDROID_SERIAL",
                        many.iter()
                            .map(|(s, _)| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            }
        }
        Ok(driver)
    }

    fn list_devices(&self) -> Result<Vec<(String, String)>, DriverError> {
        let out = process::run_ok(&self.adb, &["devices"], process::DEFAULT_TIMEOUT)?;
        Ok(parse_devices(&out.stdout_text()))
    }

    fn adb(&self, args: &[&str], timeout: Duration) -> Result<process::Output, DriverError> {
        let mut full: Vec<&str> = vec!["-s", &self.serial];
        full.extend_from_slice(args);
        process::run_ok(&self.adb, &full, timeout)
    }

    fn shell(&self, command: &str) -> Result<String, DriverError> {
        Ok(self
            .adb(&["shell", command], process::DEFAULT_TIMEOUT)?
            .stdout_text())
    }

    fn input(&self, command: &str) -> Result<(), DriverError> {
        let out = self.shell(&format!("input {command}"))?;
        let lower = out.to_ascii_lowercase();
        if lower.contains("error") || lower.contains("exception") || lower.contains("usage:") {
            return Err(DriverError::Failed(format!(
                "adb input {command}: {}",
                process::tail(&out, 300)
            )));
        }
        Ok(())
    }

    fn screen_size(&mut self) -> Result<(u32, u32), DriverError> {
        if let Some(size) = self.size {
            return Ok(size);
        }
        let frame = self.screenshot()?;
        let img = crate::frame::decode(&frame.bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        let size = img.dimensions();
        self.size = Some(size);
        Ok(size)
    }

    fn swipe(&self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        self.input(&format!(
            "swipe {} {} {} {} {}",
            from.x.round() as i64,
            from.y.round() as i64,
            to.x.round() as i64,
            to.y.round() as i64,
            duration_ms.max(1)
        ))
    }
}

fn keycode(key: &Key) -> Result<String, DriverError> {
    Ok(match key {
        Key::Named(named) => match named {
            NamedKey::Enter => "KEYCODE_ENTER",
            NamedKey::Tab => "KEYCODE_TAB",
            NamedKey::Escape => "KEYCODE_ESCAPE",
            NamedKey::Backspace => "KEYCODE_DEL",
            NamedKey::Delete => "KEYCODE_FORWARD_DEL",
            NamedKey::Space => "KEYCODE_SPACE",
            NamedKey::Up => "KEYCODE_DPAD_UP",
            NamedKey::Down => "KEYCODE_DPAD_DOWN",
            NamedKey::Left => "KEYCODE_DPAD_LEFT",
            NamedKey::Right => "KEYCODE_DPAD_RIGHT",
            NamedKey::Home => "KEYCODE_MOVE_HOME",
            NamedKey::End => "KEYCODE_MOVE_END",
            NamedKey::PageUp => "KEYCODE_PAGE_UP",
            NamedKey::PageDown => "KEYCODE_PAGE_DOWN",
            NamedKey::Insert => "KEYCODE_INSERT",
            NamedKey::CapsLock => "KEYCODE_CAPS_LOCK",
            NamedKey::F(n) => return Ok(format!("KEYCODE_F{n}")),
            NamedKey::Back => "KEYCODE_BACK",
            NamedKey::AppHome => "KEYCODE_HOME",
            NamedKey::Recents => "KEYCODE_APP_SWITCH",
            NamedKey::Power => "KEYCODE_POWER",
            NamedKey::VolumeUp => "KEYCODE_VOLUME_UP",
            NamedKey::VolumeDown => "KEYCODE_VOLUME_DOWN",
            NamedKey::Menu => "KEYCODE_MENU",
        }
        .to_string(),
        Key::Char(c) => {
            let c = *c;
            if c.is_ascii_alphabetic() {
                format!("KEYCODE_{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("KEYCODE_{c}")
            } else {
                match c {
                    ' ' => "KEYCODE_SPACE",
                    ',' => "KEYCODE_COMMA",
                    '.' => "KEYCODE_PERIOD",
                    '-' => "KEYCODE_MINUS",
                    '=' => "KEYCODE_EQUALS",
                    '+' => "KEYCODE_PLUS",
                    '/' => "KEYCODE_SLASH",
                    '\\' => "KEYCODE_BACKSLASH",
                    ';' => "KEYCODE_SEMICOLON",
                    '\'' => "KEYCODE_APOSTROPHE",
                    '[' => "KEYCODE_LEFT_BRACKET",
                    ']' => "KEYCODE_RIGHT_BRACKET",
                    '`' => "KEYCODE_GRAVE",
                    '@' => "KEYCODE_AT",
                    '#' => "KEYCODE_POUND",
                    '*' => "KEYCODE_STAR",
                    '\t' => "KEYCODE_TAB",
                    '\n' => "KEYCODE_ENTER",
                    other => {
                        return Err(DriverError::Failed(format!(
                            "no Android keycode for `{other}`; use computer_type for text"
                        )));
                    }
                }
                .to_string()
            }
        }
    })
}

/// Escape text for `input text` (spaces become `%s`; the shell sees a
/// single-quoted string).
pub fn input_text_arg(text: &str) -> String {
    sh_quote(&text.replace(' ', "%s"))
}

impl Driver for AndroidDriver {
    fn info(&mut self) -> Result<TargetInfo, DriverError> {
        let (w, h) = self.screen_size()?;
        let mut notes = vec![
            format!("adb: {}", self.adb.display()),
            format!("serial: {}", self.serial),
        ];
        if let Ok(model) = self.shell("getprop ro.product.model") {
            let model = model.trim();
            if !model.is_empty() {
                notes.push(format!("model: {model}"));
            }
        }
        if let Ok(release) = self.shell("getprop ro.build.version.release") {
            let release = release.trim();
            if !release.is_empty() {
                notes.push(format!("android: {release}"));
            }
        }
        notes.push(
            "typing is ASCII-only through adb; double-tap is two taps; right-click = long press"
                .to_string(),
        );
        Ok(TargetInfo {
            kind: TargetKind::Android,
            driver: "adb".into(),
            device_w: w,
            device_h: h,
            notes,
            supports_ui_tree: true,
            supports_apps: true,
        })
    }

    fn screenshot(&mut self) -> Result<RawFrame, DriverError> {
        let out = self.adb(&["exec-out", "screencap", "-p"], SHOT_TIMEOUT)?;
        if !out.stdout.starts_with(b"\x89PNG") {
            return Err(DriverError::Failed(format!(
                "screencap did not return a PNG ({} bytes): {}",
                out.stdout.len(),
                process::tail(&out.stderr, 200)
            )));
        }
        let bytes = out.stdout;
        let img = crate::frame::decode(&bytes).map_err(DriverError::Failed)?;
        use image::GenericImageView;
        self.size = Some(img.dimensions());
        Ok(RawFrame { bytes })
    }

    fn move_to(&mut self, _p: Point) -> Result<(), DriverError> {
        // Touch screens have no hover; report success so the post-action
        // screenshot still happens.
        Ok(())
    }

    fn click(
        &mut self,
        p: Point,
        button: Button,
        clicks: u32,
        hold_ms: u64,
    ) -> Result<(), DriverError> {
        let hold = if hold_ms > 0 {
            hold_ms
        } else if button == Button::Right {
            800
        } else {
            0
        };
        if hold > 0 {
            return self.swipe(p, p, hold);
        }
        for _ in 0..clicks.max(1) {
            self.input(&format!(
                "tap {} {}",
                p.x.round() as i64,
                p.y.round() as i64
            ))?;
        }
        Ok(())
    }

    fn drag(&mut self, from: Point, to: Point, duration_ms: u64) -> Result<(), DriverError> {
        self.swipe(from, to, duration_ms.max(50))
    }

    fn scroll(&mut self, p: Point, dir: ScrollDir, amount: u32) -> Result<(), DriverError> {
        let (w, h) = self.screen_size()?;
        let step_y = f64::from(h) * 0.12 * f64::from(amount.max(1));
        let step_x = f64::from(w) * 0.12 * f64::from(amount.max(1));
        let (dx, dy) = match dir {
            ScrollDir::Down => (0.0, -step_y),
            ScrollDir::Up => (0.0, step_y),
            ScrollDir::Right => (-step_x, 0.0),
            ScrollDir::Left => (step_x, 0.0),
        };
        let clamp = |v: f64, max: u32| v.clamp(1.0, f64::from(max.saturating_sub(2)));
        // Center the gesture on the point so it stays on screen.
        let from = Point {
            x: clamp(p.x - dx / 2.0, w),
            y: clamp(p.y - dy / 2.0, h),
        };
        let to = Point {
            x: clamp(p.x + dx / 2.0, w),
            y: clamp(p.y + dy / 2.0, h),
        };
        self.swipe(from, to, 300)
    }

    fn type_text(&mut self, text: &str) -> Result<(), DriverError> {
        if !text.is_ascii() {
            return Err(DriverError::Failed(
                "adb `input text` only types ASCII; paste non-ASCII text through the clipboard or install ADBKeyboard".to_string(),
            ));
        }
        for (i, segment) in text.split('\n').enumerate() {
            if i > 0 {
                self.input("keyevent KEYCODE_ENTER")?;
            }
            if segment.is_empty() {
                continue;
            }
            self.input(&format!("text {}", input_text_arg(segment)))?;
        }
        Ok(())
    }

    fn key(&mut self, combo: &KeyCombo) -> Result<(), DriverError> {
        let code = keycode(&combo.key)?;
        let mut codes: Vec<String> = Vec::new();
        if combo.modifiers.ctrl {
            codes.push("KEYCODE_CTRL_LEFT".into());
        }
        if combo.modifiers.alt {
            codes.push("KEYCODE_ALT_LEFT".into());
        }
        if combo.modifiers.shift || matches!(combo.key, Key::Char(c) if c.is_ascii_uppercase()) {
            codes.push("KEYCODE_SHIFT_LEFT".into());
        }
        if combo.modifiers.meta {
            codes.push("KEYCODE_META_LEFT".into());
        }
        if codes.is_empty() {
            self.input(&format!("keyevent {code}"))
        } else {
            codes.push(code);
            self.input(&format!("keycombination {}", codes.join(" ")))
        }
    }

    fn ui_tree(&mut self) -> Result<Vec<UiNode>, DriverError> {
        let out = self.adb(
            &["exec-out", "uiautomator", "dump", "/dev/tty"],
            SHOT_TIMEOUT,
        )?;
        let xml = out.stdout_text();
        if !xml.contains("<hierarchy") {
            // Older builds cannot dump to a tty; use the sdcard path.
            self.shell("uiautomator dump /sdcard/codewhale_ui.xml")?;
            let xml = self.shell("cat /sdcard/codewhale_ui.xml")?;
            let _ = self.shell("rm -f /sdcard/codewhale_ui.xml");
            return Ok(parse_uiautomator(&xml));
        }
        Ok(parse_uiautomator(&xml))
    }

    fn app(&mut self, action: AppAction<'_>) -> Result<String, DriverError> {
        match action {
            AppAction::Launch(name) => {
                let package = if name.contains('.') {
                    name.to_string()
                } else {
                    let packages = self.shell("pm list packages")?;
                    let needle = name.to_ascii_lowercase();
                    let matches: Vec<String> = packages
                        .lines()
                        .filter_map(|l| l.strip_prefix("package:"))
                        .map(|p| p.trim().to_string())
                        .filter(|p| p.to_ascii_lowercase().contains(&needle))
                        .collect();
                    match matches.as_slice() {
                        [one] => one.clone(),
                        [] => {
                            return Err(DriverError::Failed(format!(
                                "no installed package matches `{name}`"
                            )));
                        }
                        many => {
                            return Err(DriverError::Failed(format!(
                                "`{name}` matches several packages; launch one of: {}",
                                many.join(", ")
                            )));
                        }
                    }
                };
                let out = self.shell(&format!(
                    "monkey -p {} -c android.intent.category.LAUNCHER 1",
                    sh_quote(&package)
                ))?;
                if out.contains("No activities found") || out.contains("monkey aborted") {
                    return Err(DriverError::Failed(format!(
                        "could not launch {package}: {}",
                        process::tail(&out, 200)
                    )));
                }
                Ok(format!("launched {package}"))
            }
            AppAction::List => {
                let out = self.shell("pm list packages -3")?;
                let mut names: Vec<&str> = out
                    .lines()
                    .filter_map(|l| l.strip_prefix("package:"))
                    .map(str::trim)
                    .collect();
                names.sort_unstable();
                Ok(format!(
                    "{} third-party packages:\n{}",
                    names.len(),
                    names.join("\n")
                ))
            }
            AppAction::Current => {
                let out = self.shell("dumpsys window")?;
                let focus = out
                    .lines()
                    .find(|l| l.contains("mCurrentFocus") || l.contains("mFocusedApp"))
                    .map(|l| l.trim().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                Ok(format!("current: {focus}"))
            }
        }
    }

    fn devices(&mut self) -> Result<String, DriverError> {
        let out = process::run_ok(&self.adb, &["devices", "-l"], process::DEFAULT_TIMEOUT)?;
        Ok(format!(
            "selected: {}\n{}",
            self.serial,
            out.stdout_text().trim()
        ))
    }

    fn element(&mut self) -> Option<&mut dyn ElementDriver> {
        Some(self)
    }
}

fn unescape_xml(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(idx) = rest.find('&') {
        out.push_str(&rest[..idx]);
        rest = &rest[idx..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..end];
        match entity {
            "quot" => out.push('"'),
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "apos" => out.push('\''),
            _ => {
                let code = entity
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()));
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str(&rest[..=end]),
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let bytes = tag.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i || i >= bytes.len() {
            break;
        }
        let key = &tag[start..i];
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        i += 1;
        let vstart = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        let value = &tag[vstart..i.min(bytes.len())];
        attrs.push((key.to_string(), unescape_xml(value)));
        i += 1;
    }
    attrs
}

/// `[l,t][r,b]` → tuple. Negative coordinates (elements partly scrolled
/// off-screen) are clamped to 0 rather than losing their sign.
pub fn parse_bounds(value: &str) -> Option<(u32, u32, u32, u32)> {
    let mut nums: Vec<u32> = Vec::new();
    let mut current = String::new();
    let mut negative = false;
    let flush = |current: &mut String, negative: &mut bool, nums: &mut Vec<u32>| {
        if !current.is_empty() {
            let parsed: u32 = current.parse().unwrap_or(u32::MAX);
            nums.push(if *negative { 0 } else { parsed });
            current.clear();
        }
        *negative = false;
    };
    for c in value.chars() {
        if c.is_ascii_digit() {
            current.push(c);
        } else {
            flush(&mut current, &mut negative, &mut nums);
            negative = c == '-';
        }
    }
    flush(&mut current, &mut negative, &mut nums);
    if nums.len() == 4 && nums[0] <= nums[2] && nums[1] <= nums[3] {
        Some((nums[0], nums[1], nums[2], nums[3]))
    } else {
        None
    }
}

/// Parse a `uiautomator dump` hierarchy into flat nodes (document order).
pub fn parse_uiautomator(xml: &str) -> Vec<UiNode> {
    let mut nodes = Vec::new();
    let mut depth: u8 = 0;
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if tag.starts_with("/node") {
            depth = depth.saturating_sub(1);
            continue;
        }
        let Some(body) = tag.strip_prefix("node") else {
            continue;
        };
        let self_closing = body.trim_end().ends_with('/');
        let body = body.trim_end().trim_end_matches('/');
        let attrs = attributes(body);
        let get = |name: &str| {
            attrs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        };
        let is_true = |name: &str| get(name) == "true";
        if let Some(bounds) = parse_bounds(get("bounds")) {
            let class_full = get("class");
            let class = class_full
                .rsplit('.')
                .next()
                .unwrap_or(class_full)
                .to_string();
            let text = get("text");
            let label = if text.is_empty() {
                get("content-desc")
            } else {
                text
            };
            nodes.push(UiNode {
                editable: class_full.contains("EditText") || is_true("password"),
                class,
                label: label.chars().take(80).collect(),
                id: get("resource-id").to_string(),
                bounds,
                clickable: is_true("clickable")
                    || is_true("long-clickable")
                    || is_true("checkable"),
                scrollable: is_true("scrollable"),
                focused: is_true("focused"),
                depth,
            });
        }
        if !self_closing {
            depth = depth.saturating_add(1);
        }
    }
    nodes
}

// ---------------------------------------------------------------------------
// Element surface (phase 2): the uiautomator tree, addressed by index.
// ---------------------------------------------------------------------------

impl ElementDriver for AndroidDriver {
    fn apps(&mut self) -> Result<Vec<AppInfo>, DriverError> {
        let out = self.shell("ps -A -o PID,NAME")?;
        let packages = device_elements::parse_running_packages(&out);
        if packages.is_empty() {
            return Err(DriverError::Failed(
                "`ps -A -o PID,NAME` listed no app processes; computer_app with action=list shows installed packages instead".to_string(),
            ));
        }
        let (w, h) = self.screen_size()?;
        Ok(packages
            .into_iter()
            .map(|(pid, package)| {
                let identity = device_elements::device_identity(pid, &package);
                AppInfo {
                    windows: vec![crate::elements::WindowInfo {
                        id: 0,
                        title: identity.label(),
                        x: 0,
                        y: 0,
                        w,
                        h,
                    }],
                    identity,
                }
            })
            .collect())
    }

    fn app_state(&mut self, app: &AppIdentity, opts: &StateOpts) -> Result<AppState, DriverError> {
        let device = self.screen_size()?;
        let state = device_elements::snapshot(self, app, opts, device)?;
        self.element_nodes = state.nodes.clone();
        Ok(state)
    }

    fn act(
        &mut self,
        app: &AppIdentity,
        action: ElementAction,
    ) -> Result<ActionReceipt, DriverError> {
        let nodes = std::mem::take(&mut self.element_nodes);
        let outcome = device_elements::act(self, app, action, &nodes);
        self.element_nodes = nodes;
        outcome
    }

    fn raise(&mut self, app: &AppIdentity) -> Result<(), DriverError> {
        let package = if app.bundle_id.is_empty() {
            app.name.clone()
        } else {
            app.bundle_id.clone()
        };
        Driver::app(self, AppAction::Launch(&package))?;
        Ok(())
    }

    fn caps(&self) -> ElementCaps {
        device_elements::caps(
            "Android: uiautomator tree with indexed taps and text; actions go to the foreground app, not the background",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_list() {
        let out = "* daemon started successfully\nList of devices attached\nemulator-5554\tdevice\nR58M1234\toffline\n\n";
        assert_eq!(
            parse_devices(out),
            vec![
                ("emulator-5554".to_string(), "device".to_string()),
                ("R58M1234".to_string(), "offline".to_string())
            ]
        );
    }

    #[test]
    fn parses_uiautomator_dump() {
        let xml = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.app" content-desc="" checkable="false" checked="false" clickable="false" enabled="true" focusable="false" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[0,0][1080,2400]"><node index="0" text="Say &quot;hi&quot; &amp; go" resource-id="com.app:id/ok" class="android.widget.Button" package="com.app" content-desc="" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="false" scrollable="false" long-clickable="false" password="false" selected="false" bounds="[100,200][300,260]" /><node index="1" text="" resource-id="com.app:id/name" class="android.widget.EditText" package="com.app" content-desc="Name" checkable="false" checked="false" clickable="true" enabled="true" focusable="true" focused="true" scrollable="false" long-clickable="true" password="false" selected="false" bounds="[100,300][900,380]" /></node></hierarchy>UI hierchary dumped to: /dev/tty"#;
        let nodes = parse_uiautomator(xml);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].class, "FrameLayout");
        assert_eq!(nodes[0].depth, 0);
        assert!(!nodes[0].is_interesting());
        assert_eq!(nodes[1].label, "Say \"hi\" & go");
        assert_eq!(nodes[1].id, "com.app:id/ok");
        assert_eq!(nodes[1].bounds, (100, 200, 300, 260));
        assert!(nodes[1].clickable);
        assert_eq!(nodes[1].depth, 1);
        assert_eq!(nodes[2].label, "Name");
        assert!(nodes[2].editable && nodes[2].focused);
        assert_eq!(nodes[2].center(), (500, 340));
    }

    #[test]
    fn bounds_keep_sign_semantics() {
        assert_eq!(
            parse_bounds("[100,200][300,260]"),
            Some((100, 200, 300, 260))
        );
        assert_eq!(parse_bounds("[-10,-50][1080,200]"), Some((0, 0, 1080, 200)));
        assert_eq!(parse_bounds("[10,50][5,200]"), None);
        assert_eq!(parse_bounds(""), None);
    }

    #[test]
    fn input_text_escaping() {
        assert_eq!(input_text_arg("hello world"), "'hello%sworld'");
        assert_eq!(input_text_arg("a'b"), "'a'\\''b'");
        assert_eq!(input_text_arg("plain"), "plain");
    }

    #[test]
    fn keycodes_cover_named_and_chars() {
        assert_eq!(
            keycode(&Key::Named(NamedKey::Back)).unwrap(),
            "KEYCODE_BACK"
        );
        assert_eq!(keycode(&Key::Named(NamedKey::F(3))).unwrap(), "KEYCODE_F3");
        assert_eq!(keycode(&Key::Char('a')).unwrap(), "KEYCODE_A");
        assert_eq!(keycode(&Key::Char('7')).unwrap(), "KEYCODE_7");
        assert!(keycode(&Key::Char('é')).is_err());
    }
}
